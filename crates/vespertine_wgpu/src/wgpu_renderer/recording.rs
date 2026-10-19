//! Frame recording: packed instance upload (PRF-007 R1), cached bindings (R2), tracked pass
//! state (R3), grouped path rasterization (R4), cached global uniforms (R5), retained scratch
//! storage (R6), the ordered backdrop-blur effect pass (RND-003) and the bounded-resource
//! counters of PRF-010.

use super::*;
use crate::wgpu_blur::CompositePlan;

/// Section indices of the packed instance buffer.
const SECTION_QUADS: usize = 0;
const SECTION_SHADOWS: usize = 1;
const SECTION_UNDERLINES: usize = 2;
const SECTION_MONO_SPRITES: usize = 3;
const SECTION_SUBPIXEL_SPRITES: usize = 4;
const SECTION_POLY_SPRITES: usize = 5;
const SECTION_PATH_VERTICES: usize = 6;
const SECTION_PATH_SPRITES: usize = 7;
const SECTION_COUNT: usize = 8;

/// Minimum size of every section: empty bindings are rejected.
const MIN_SECTION_SIZE: u64 = 16;

/// Binding identities of the two global bind groups.
const GLOBALS_BINDING: u64 = 1;
const PATH_GLOBALS_BINDING: u64 = 2;
/// Identity of the globals bind group of a frame whose scene has curved overflow clip shapes.
const CLIP_GLOBALS_BINDING: u64 = 3;
/// First identity handed out to cached bind groups.
const FIRST_CACHED_BINDING: u64 = 16;

/// Cached bindings unused for more than this many frames are released.
const BINDING_RETENTION_FRAMES: u64 = 2;
/// Path batches grouped into one rasterization pass at most.
pub(super) const MAX_PATH_GROUP: usize = 64;
/// Margin around path batch bounds when testing separation.
const PATH_SEPARATION_MARGIN: f32 = 2.0;

/// Pipelines used by a frame, with the slot layout each exposes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PipelineKind {
    Quads,
    QuadsClippedSolid,
    QuadsOpaqueSolid,
    Shadows,
    Underlines,
    PathRasterization,
    Paths,
    MonoSprites,
    SubpixelSprites,
    PolySprites,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotLayout {
    None,
    Globals,
    Instances,
    Texture,
}

impl PipelineKind {
    /// Bind group layouts of the four tracked slots.
    fn slot_layouts(self) -> [SlotLayout; 4] {
        use SlotLayout::*;
        match self {
            PipelineKind::Quads
            | PipelineKind::QuadsClippedSolid
            | PipelineKind::QuadsOpaqueSolid
            | PipelineKind::Shadows
            | PipelineKind::Underlines
            | PipelineKind::PathRasterization => [Globals, Instances, None, None],
            PipelineKind::Paths
            | PipelineKind::MonoSprites
            | PipelineKind::SubpixelSprites
            | PipelineKind::PolySprites => [Globals, Instances, Texture, None],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(super) enum TextureKey {
    Atlas(AtlasTextureId, u64),
    PathIntermediate(u64),
    /// A scroll-layer tile texture, by the serial of the texture (PRF-004).
    Tile(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct InstanceKey {
    buffer_generation: u64,
    offset: u64,
    size: u64,
}

struct BindingEntry {
    bind_group: wgpu::BindGroup,
    id: u64,
    last_used: u64,
}

/// Bind groups cached by the identity of the underlying texture view or instance buffer range.
#[derive(Default)]
pub(super) struct BindingCache {
    frame: u64,
    next_id: u64,
    instances: FxHashMap<InstanceKey, BindingEntry>,
    textures: FxHashMap<TextureKey, BindingEntry>,
}

impl BindingCache {
    fn begin_frame(&mut self) {
        self.frame += 1;
        if self.next_id < FIRST_CACHED_BINDING {
            self.next_id = FIRST_CACHED_BINDING;
        }
    }

    /// Releases entries unused for more than the retention window.
    fn end_frame(&mut self) {
        let frame = self.frame;
        self.instances
            .retain(|_, entry| frame - entry.last_used <= BINDING_RETENTION_FRAMES);
        self.textures
            .retain(|_, entry| frame - entry.last_used <= BINDING_RETENTION_FRAMES);
    }

    fn clear_instances(&mut self) {
        self.instances.clear();
    }

    #[cfg(any(test, feature = "bench-support", feature = "test-support"))]
    pub(super) fn len(&self) -> usize {
        self.instances.len() + self.textures.len()
    }
}

/// Counters of the recording path, available to tests and the performance scenes. Plain
/// increments; production code never depends on them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordStats {
    /// Rendered frames.
    pub frames: u64,
    /// Instance-data staging writes (one per frame).
    pub instance_writes: u64,
    /// Bytes of packed instance data written.
    pub instance_bytes: u64,
    /// Bind groups created.
    pub binding_creations: u64,
    /// Set-pipeline commands issued.
    pub pipeline_sets: u64,
    /// Set-bind-group commands issued.
    pub bind_group_sets: u64,
    /// Render passes begun (rasterization, main, effect filter and presentation passes).
    pub render_passes: u64,
    /// Passes that rasterize path groups.
    pub path_rasterization_passes: u64,
    /// Global-uniform buffer uploads.
    pub global_uploads: u64,
    /// Path intermediate allocations (resolve texture, plus multisample texture).
    pub path_intermediate_allocations: u64,
    /// Instance-buffer replacements: growth and shrink.
    pub instance_buffer_replacements: u64,
}

/// The value last uploaded for one global uniform.
pub(super) struct Cached<T>(Option<T>);

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T: Pod> Cached<T> {
    /// Records `value` and reports whether it differs from the previously uploaded bytes.
    fn changes(&mut self, value: T) -> bool {
        let unchanged = self
            .0
            .as_ref()
            .is_some_and(|last| bytemuck::bytes_of(last) == bytemuck::bytes_of(&value));
        self.0 = Some(value);
        !unchanged
    }
}

/// The previously uploaded bytes of each global uniform value (PRF-007 R5).
#[derive(Default)]
pub(super) struct UniformCache {
    globals: Cached<GlobalParams>,
    path_globals: Cached<GlobalParams>,
    gamma: Cached<GammaParams>,
}

impl UniformCache {
    pub(super) fn invalidate(&mut self) {
        *self = Self::default();
    }
}

/// Largest absolute coordinate the specialized solid routes trust (PRF-008 R2, R5).
const SOLID_COORDINATE_LIMIT: f32 = 1_048_576.0;
/// Smallest integer interior area, in device pixels, that is split off a quad (PRF-008 R6).
const LARGE_INTERIOR_AREA: u64 = 16_384;

/// The shading route of a quad or quad batch (PRF-008 R1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillClass {
    /// The general quad shader.
    General,
    /// Clipped solid shading: simple solid quads, clipped by the rectangular content mask.
    ClippedSolid,
    /// Opaque solid shading: blending disabled, no per-fragment clipping.
    OpaqueSolid,
}

/// One recorded quad draw sequence, for tests (PRF-008 test-support access).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FillRecord {
    /// Quads of the scene covered by the sequence.
    pub quads: Range<usize>,
    /// Route of the sequence. A split quad reports the route of its interior.
    pub class: FillClass,
    /// Draw calls issued.
    pub draws: u32,
    /// Scissor rectangles set, the restoring one included.
    pub scissors: u32,
    /// Whether the quad was split into exterior strips and an interior (software adapters).
    pub split: bool,
}

/// Whether `quad` has a solid background, square corners, no border and no fade band with a
/// positive width. The scoped rounded clip fields are deliberately not inspected (the
/// plain solid quirk of RND-001 R11a, RND-002 R6, PRF-008 R2).
pub(super) fn is_simple_solid(quad: &Quad) -> bool {
    let radii = quad.corner_radii;
    let widths = quad.border_widths;
    quad.background.as_solid().is_some()
        && radii.top_left.0 == 0.0
        && radii.top_right.0 == 0.0
        && radii.bottom_right.0 == 0.0
        && radii.bottom_left.0 == 0.0
        && widths.top.0 == 0.0
        && widths.right.0 == 0.0
        && widths.bottom.0 == 0.0
        && widths.left.0 == 0.0
        && !quad.scope.fade_active()
}

/// A simple solid quad of alpha exactly 1 whose outward-rounded bounds lie inside its
/// rectangular content mask, with finite, bounded geometry (PRF-008 R2). A quad inside a
/// curved overflow clip never qualifies: that clip needs the per-fragment route.
pub(super) fn is_opaque_solid(quad: &Quad) -> bool {
    if !is_simple_solid(quad) || quad.scope.curved_clip != 0 {
        return false;
    }
    if quad.background.as_solid().map(|color| color.a) != Some(1.0) {
        return false;
    }
    let bounds = quad.bounds;
    let mask = quad.content_mask.bounds;
    let (x, y) = (bounds.origin.x.0, bounds.origin.y.0);
    let (width, height) = (bounds.size.width.0, bounds.size.height.0);
    let (mask_x, mask_y) = (mask.origin.x.0, mask.origin.y.0);
    let (mask_width, mask_height) = (mask.size.width.0, mask.size.height.0);
    let finite = [x, y, width, height, mask_x, mask_y, mask_width, mask_height]
        .iter()
        .all(|value| value.is_finite());
    if !finite || width <= 0.0 || height <= 0.0 {
        return false;
    }
    if [x, y, width, height]
        .iter()
        .any(|value| value.abs() > SOLID_COORDINATE_LIMIT)
    {
        return false;
    }
    x.floor() >= mask_x
        && y.floor() >= mask_y
        && (x + width).ceil() <= mask_x + mask_width
        && (y + height).ceil() <= mask_y + mask_height
}

/// The route of one quad on its own.
pub(super) fn quad_fill_class(quad: &Quad) -> FillClass {
    if is_opaque_solid(quad) {
        FillClass::OpaqueSolid
    } else if is_simple_solid(quad) {
        FillClass::ClippedSolid
    } else {
        FillClass::General
    }
}

/// The route of a whole hardware-adapter batch: opaque only when every member is opaque
/// solid, clipped solid only when every member is simple solid, general otherwise
/// (PRF-008 R3).
pub(super) fn batch_fill_class(quads: &[Quad]) -> FillClass {
    let mut class = FillClass::OpaqueSolid;
    for quad in quads {
        match quad_fill_class(quad) {
            FillClass::General => return FillClass::General,
            FillClass::ClippedSolid => class = FillClass::ClippedSolid,
            FillClass::OpaqueSolid => {}
        }
    }
    class
}

/// The integer interior of a large-interior candidate (PRF-008 R5, R6): the device-pixel
/// rectangle `[left, top, right, bottom)` that general shading would color with the plain
/// solid fill, when it covers at least 16,384 pixels.
pub(super) fn large_interior(quad: &Quad, drawable: (u32, u32)) -> Option<[u32; 4]> {
    if quad.background.as_solid().is_none() || is_simple_solid(quad) {
        return None;
    }
    let bounds = quad.bounds;
    let mask = quad.content_mask.bounds;
    let scope = &quad.scope;
    let radii = quad.corner_radii;
    let widths = quad.border_widths;
    let radii = [
        radii.top_left.0,
        radii.top_right.0,
        radii.bottom_right.0,
        radii.bottom_left.0,
    ];
    let widths = [widths.top.0, widths.right.0, widths.bottom.0, widths.left.0];
    let geometry = [
        bounds.origin.x.0,
        bounds.origin.y.0,
        bounds.size.width.0,
        bounds.size.height.0,
    ];
    let mask_values = [
        mask.origin.x.0,
        mask.origin.y.0,
        mask.size.width.0,
        mask.size.height.0,
    ];
    let finite = geometry
        .iter()
        .chain(&mask_values)
        .chain(&scope.fade_edges)
        .chain(&scope.fade_bands)
        .chain(&radii)
        .chain(&widths)
        .all(|value| value.is_finite());
    if !finite {
        return None;
    }
    if geometry
        .iter()
        .chain(&scope.fade_edges)
        .chain(&scope.fade_bands)
        .any(|value| value.abs() > SOLID_COORDINATE_LIMIT)
    {
        return None;
    }
    let g = radii
        .iter()
        .chain(&widths)
        .fold(0.0_f32, |acc, value| acc.max(*value))
        + 1.0;
    let mut left = geometry[0] + g;
    let mut top = geometry[1] + g;
    let mut right = geometry[0] + geometry[2] - g;
    let mut bottom = geometry[1] + geometry[3] - g;
    let [edge_top, edge_bottom, edge_left, edge_right] = scope.fade_edges;
    let [band_top, band_bottom, band_left, band_right] = scope.fade_bands;
    if band_top > 0.0 {
        top = top.max(edge_top + band_top + 1.0);
    }
    if band_bottom > 0.0 {
        bottom = bottom.min(edge_bottom - band_bottom - 1.0);
    }
    if band_left > 0.0 {
        left = left.max(edge_left + band_left + 1.0);
    }
    if band_right > 0.0 {
        right = right.min(edge_right - band_right - 1.0);
    }
    left = left.max(mask_values[0]);
    top = top.max(mask_values[1]);
    right = right.min(mask_values[0] + mask_values[2]);
    bottom = bottom.min(mask_values[1] + mask_values[3]);
    if ![left, top, right, bottom]
        .iter()
        .all(|value| value.is_finite())
    {
        return None;
    }
    let (width, height) = (drawable.0 as f32, drawable.1 as f32);
    let x0 = left.ceil().clamp(0.0, width) as u32;
    let y0 = top.ceil().clamp(0.0, height) as u32;
    let x1 = right.floor().clamp(0.0, width) as u32;
    let y1 = bottom.floor().clamp(0.0, height) as u32;
    let area = u64::from(x1.saturating_sub(x0)) * u64::from(y1.saturating_sub(y0));
    (area >= LARGE_INTERIOR_AREA).then_some([x0, y0, x1, y1])
}

struct PathBatchPlan {
    /// Vertices of the batch within the path-vertex section.
    vertices: Range<u32>,
    /// Composite sprites of the batch within the path-sprite section.
    sprites: Range<u32>,
    group: usize,
}

struct PathGroup {
    /// Consecutive path batches rasterized together.
    batches: Range<usize>,
    vertices: Range<u32>,
}

/// CPU scratch retained across frames (PRF-007 R6).
#[derive(Default)]
pub(super) struct FrameScratch {
    batches: Vec<PrimitiveBatch>,
    path_vertices: Vec<PathRasterizationVertex>,
    path_sprites: Vec<PathSprite>,
    path_batches: Vec<PathBatchPlan>,
    path_groups: Vec<PathGroup>,
    group_bounds: Vec<Bounds<ScaledPixels>>,
}

impl FrameScratch {
    fn clear(&mut self) {
        self.batches.clear();
        self.path_vertices.clear();
        self.path_sprites.clear();
        self.path_batches.clear();
        self.path_groups.clear();
        self.group_bounds.clear();
    }
}

#[derive(Clone, Copy, Default)]
struct Section {
    offset: u64,
    size: u64,
}

struct FrameLayout {
    sections: [Section; SECTION_COUNT],
    total: u64,
}

/// The tracked pipeline and bindings of the current render pass (PRF-007 R3).
#[derive(Default)]
struct PassState {
    pipeline: Option<PipelineKind>,
    /// Identity and dynamic offset of the content of each tracked slot.
    slots: [Option<(u64, u32)>; 4],
}

impl PassState {
    fn reset(&mut self) {
        *self = Self::default();
    }
}

struct FrameEncoder {
    encoder: wgpu::CommandEncoder,
    pass: Option<wgpu::RenderPass<'static>>,
    state: PassState,
    render_passes: u64,
    pipeline_sets: u64,
    bind_group_sets: u64,
}

impl FrameEncoder {
    fn begin_pass(&mut self, descriptor: &wgpu::RenderPassDescriptor<'_>) {
        self.end_pass();
        self.pass = Some(self.encoder.begin_render_pass(descriptor).forget_lifetime());
        self.state.reset();
        self.render_passes += 1;
    }

    fn begin_main_pass(&mut self, view: &wgpu::TextureView, load: wgpu::LoadOp<wgpu::Color>) {
        self.begin_pass(&wgpu::RenderPassDescriptor {
            label: Some("main_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
    }

    fn end_pass(&mut self) {
        self.pass = None;
    }

    fn pass(&mut self) -> &mut wgpu::RenderPass<'static> {
        self.pass.as_mut().expect("a render pass is active")
    }

    fn set_pipeline(&mut self, kind: PipelineKind, pipeline: &wgpu::RenderPipeline) {
        if self.state.pipeline == Some(kind) {
            return;
        }
        // Earlier slots stay bound across a pipeline switch only while their layouts are
        // identical; everything from the first difference on must be set again.
        match self.state.pipeline {
            Some(previous) => {
                let old = previous.slot_layouts();
                let new = kind.slot_layouts();
                if let Some(first) = (0..4).find(|&slot| old[slot] != new[slot]) {
                    for tracked in &mut self.state.slots[first..] {
                        *tracked = None;
                    }
                }
            }
            None => self.state.slots = [None; 4],
        }
        self.state.pipeline = Some(kind);
        self.pass().set_pipeline(pipeline);
        self.pipeline_sets += 1;
    }

    fn set_bind_group(&mut self, slot: usize, id: u64, bind_group: &wgpu::BindGroup) {
        if self.state.slots[slot] == Some((id, 0)) {
            return;
        }
        self.state.slots[slot] = Some((id, 0));
        self.pass().set_bind_group(slot as u32, bind_group, &[]);
        self.bind_group_sets += 1;
    }
}

fn separated(a: &Bounds<ScaledPixels>, b: &Bounds<ScaledPixels>) -> bool {
    let (ax1, ay1) = (
        a.origin.x.0 + a.size.width.0,
        a.origin.y.0 + a.size.height.0,
    );
    let (bx1, by1) = (
        b.origin.x.0 + b.size.width.0,
        b.origin.y.0 + b.size.height.0,
    );
    // No positive-area intersection.
    !(a.origin.x.0.max(b.origin.x.0) < ax1.min(bx1)
        && a.origin.y.0.max(b.origin.y.0) < ay1.min(by1))
}

fn struct_size<T>() -> u64 {
    std::mem::size_of::<T>() as u64
}

impl WgpuRendererCore {
    /// Creates (or fetches) the instance binding for one section range.
    fn instance_binding(&mut self, section: Section) {
        let key = InstanceKey {
            buffer_generation: self.instance_buffer_generation,
            offset: section.offset,
            size: section.size,
        };
        let frame = self.bindings.frame;
        if let Some(entry) = self.bindings.instances.get_mut(&key) {
            entry.last_used = frame;
            return;
        }
        let resources = &self.resources;
        let bind_group = resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("instances_bind_group"),
                layout: &resources.bind_group_layouts.instances,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &resources.instance_buffer,
                        offset: section.offset,
                        size: NonZeroU64::new(section.size),
                    }),
                }],
            });
        let id = self.bindings.next_id;
        self.bindings.next_id += 1;
        self.stats.binding_creations += 1;
        self.bindings.instances.insert(
            key,
            BindingEntry {
                bind_group,
                id,
                last_used: frame,
            },
        );
    }

    fn texture_binding(&mut self, key: TextureKey, view: &wgpu::TextureView) {
        let frame = self.bindings.frame;
        if let Some(entry) = self.bindings.textures.get_mut(&key) {
            entry.last_used = frame;
            return;
        }
        let bind_group = self.create_texture_bind_group("texture_bind_group", view);
        let id = self.bindings.next_id;
        self.bindings.next_id += 1;
        self.stats.binding_creations += 1;
        self.bindings.textures.insert(
            key,
            BindingEntry {
                bind_group,
                id,
                last_used: frame,
            },
        );
    }

    fn create_texture_bind_group(
        &self,
        label: &str,
        texture_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let resources = &self.resources;
        resources
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &resources.bind_group_layouts.texture,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&resources.atlas_sampler),
                    },
                ],
            })
    }

    /// Groups the path batches of the frame and builds their rasterization vertices and
    /// composite sprites (PRF-007 R4).
    fn plan_batches(scene: &Scene, scratch: &mut FrameScratch, group_limit: usize) {
        scratch.clear();
        for batch in scene.batches() {
            if let PrimitiveBatch::Paths(range) = &batch {
                Self::plan_path_batch(scene, range.clone(), scratch, group_limit);
            }
            scratch.batches.push(batch);
        }
    }

    fn plan_path_batch(
        scene: &Scene,
        range: Range<usize>,
        scratch: &mut FrameScratch,
        group_limit: usize,
    ) {
        let paths = &scene.paths[range];
        let vertex_start = scratch.path_vertices.len() as u32;
        let mut bounds = paths[0].clipped_bounds();
        for path in paths {
            let path_bounds = path.clipped_bounds();
            bounds = bounds.union(&path_bounds);
            scratch
                .path_vertices
                .extend(path.vertices.iter().map(|v| PathRasterizationVertex {
                    xy_position: v.xy_position,
                    st_position: v.st_position,
                    color: path.color,
                    bounds: path_bounds,
                }));
        }
        let vertex_end = scratch.path_vertices.len() as u32;

        let sprite_start = scratch.path_sprites.len() as u32;
        let first = &paths[0];
        if paths.last().map(|p| &p.order) == Some(&first.order) {
            scratch
                .path_sprites
                .extend(paths.iter().map(|p| PathSprite {
                    bounds: p.clipped_bounds(),
                    // Paths of one order are separate sprites, each cut by its own chain.
                    curved_clip: p.curved_clip,
                    pad: 0,
                }));
        } else {
            scratch.path_sprites.push(PathSprite {
                bounds,
                curved_clip: first.curved_clip,
                pad: 0,
            });
        }
        let sprite_end = scratch.path_sprites.len() as u32;

        let expanded = bounds.dilate(ScaledPixels(PATH_SEPARATION_MARGIN));
        let batch_index = scratch.path_batches.len();
        let conflicts = match scratch.path_groups.last() {
            None => true,
            Some(group) => {
                group.batches.len() >= group_limit
                    || scratch
                        .group_bounds
                        .iter()
                        .any(|member| !separated(member, &expanded))
            }
        };
        if conflicts {
            scratch.group_bounds.clear();
            scratch.path_groups.push(PathGroup {
                batches: batch_index..batch_index,
                vertices: vertex_start..vertex_start,
            });
        }
        scratch.group_bounds.push(expanded);
        let group_index = scratch.path_groups.len() - 1;
        let group = &mut scratch.path_groups[group_index];
        group.batches.end = batch_index + 1;
        group.vertices.end = vertex_end;
        scratch.path_batches.push(PathBatchPlan {
            vertices: vertex_start..vertex_end,
            sprites: sprite_start..sprite_end,
            group: group_index,
        });
    }

    fn frame_layout(&self, scene: &Scene, scratch: &FrameScratch) -> FrameLayout {
        let sizes = [
            scene.quads.len() as u64 * struct_size::<Quad>(),
            scene.shadows.len() as u64 * struct_size::<Shadow>(),
            scene.underlines.len() as u64 * struct_size::<Underline>(),
            scene.monochrome_sprites.len() as u64 * struct_size::<MonochromeSprite>(),
            scene.subpixel_sprites.len() as u64 * struct_size::<SubpixelSprite>(),
            scene.polychrome_sprites.len() as u64 * struct_size::<PolychromeSprite>(),
            scratch.path_vertices.len() as u64 * struct_size::<PathRasterizationVertex>(),
            scratch.path_sprites.len() as u64 * struct_size::<PathSprite>(),
        ];
        let alignment = self.instance_data_alignment.max(1);
        let mut sections = [Section::default(); SECTION_COUNT];
        let mut cursor = 0_u64;
        for (section, size) in sections.iter_mut().zip(sizes) {
            let offset = cursor.next_multiple_of(alignment);
            let size = size.max(MIN_SECTION_SIZE);
            *section = Section { offset, size };
            cursor = offset + size;
        }
        FrameLayout {
            sections,
            total: cursor.next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT),
        }
    }

    /// Uploads the whole frame's instance data with one staging write.
    fn write_frame_data(
        &mut self,
        scene: &Scene,
        scratch: &FrameScratch,
        layout: &FrameLayout,
    ) -> Result<()> {
        if layout.total > self.instance_data_capacity {
            self.grow_instance_data(layout.total)?;
        }
        let mut view = self
            .resources
            .queue
            .write_buffer_with(
                &self.resources.instance_buffer,
                0,
                NonZeroU64::new(layout.total).context("empty instance upload")?,
            )
            .context("instance upload does not fit the instance buffer")?;
        // Safety: every instance type is `repr(C)` plain data that the shaders read back with
        // the same layout.
        unsafe fn copy<T>(view: &mut wgpu::QueueWriteBufferView, section: Section, data: &[T]) {
            if data.is_empty() {
                return;
            }
            let bytes = unsafe {
                std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data))
            };
            let start = section.offset as usize;
            view.slice(start..start + bytes.len())
                .copy_from_slice(bytes);
        }
        let s = &layout.sections;
        unsafe {
            copy(&mut view, s[SECTION_QUADS], &scene.quads);
            copy(&mut view, s[SECTION_SHADOWS], &scene.shadows);
            copy(&mut view, s[SECTION_UNDERLINES], &scene.underlines);
            copy(
                &mut view,
                s[SECTION_MONO_SPRITES],
                &scene.monochrome_sprites,
            );
            copy(
                &mut view,
                s[SECTION_SUBPIXEL_SPRITES],
                &scene.subpixel_sprites,
            );
            copy(
                &mut view,
                s[SECTION_POLY_SPRITES],
                &scene.polychrome_sprites,
            );
            copy(&mut view, s[SECTION_PATH_VERTICES], &scratch.path_vertices);
            copy(&mut view, s[SECTION_PATH_SPRITES], &scratch.path_sprites);
        }
        drop(view);
        self.stats.instance_writes += 1;
        self.stats.instance_bytes += layout.total;
        Ok(())
    }

    fn grow_instance_data(&mut self, required: u64) -> Result<()> {
        let capacity = (self.instance_data_capacity * 2)
            .max(required.next_power_of_two())
            .min(self.max_instance_data_size);
        anyhow::ensure!(
            capacity >= required,
            "instance data needs {required} bytes, above the maximum of {}",
            self.max_instance_data_size
        );
        anyhow::ensure!(
            capacity > self.instance_data_capacity,
            "frame instance data exceeds the {}-byte maximum",
            self.max_instance_data_size
        );
        log::debug!(
            "instance data grown from {} to {capacity}",
            self.instance_data_capacity
        );
        self.replace_instance_buffer(capacity);
        Ok(())
    }

    /// Replaces the instance buffer. Cached bindings of the old buffer are dropped; work already
    /// submitted keeps the old buffer alive through the graphics API.
    fn replace_instance_buffer(&mut self, capacity: u64) {
        self.resources.instance_buffer =
            self.resources
                .device
                .create_buffer(&wgpu::BufferDescriptor {
                    label: Some("instance_buffer"),
                    size: capacity,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
        self.instance_data_capacity = capacity;
        self.instance_buffer_generation += 1;
        self.bindings.clear_instances();
        self.stats.instance_buffer_replacements += 1;
    }

    /// Records the used instance bytes of an encoded frame and shrinks the buffer after a
    /// sustained low-usage streak (PRF-010 R4).
    fn note_instance_usage(&mut self, used: u64) {
        if self.instance_data_capacity > self.instance_floor
            && used <= self.instance_data_capacity / 2
        {
            self.low_usage_frames += 1;
            if self.low_usage_frames >= INSTANCE_SHRINK_STREAK {
                let capacity = (self.instance_data_capacity / 2).max(self.instance_floor);
                self.replace_instance_buffer(capacity);
                self.low_usage_frames = 0;
            }
        } else {
            self.low_usage_frames = 0;
        }
    }

    pub(super) fn upload_globals(&mut self, size: Size<DevicePixels>, premultiplied_alpha: bool) {
        let gamma = GammaParams {
            gamma_ratios: self.rendering_params.gamma_ratios,
            grayscale_enhanced_contrast: self.rendering_params.grayscale_enhanced_contrast,
            subpixel_enhanced_contrast: self.rendering_params.subpixel_enhanced_contrast,
            is_bgr: self.is_bgr as u32,
            _pad: 0,
        };
        let globals = GlobalParams {
            viewport_size: [size.width.0 as f32, size.height.0 as f32],
            premultiplied_alpha: premultiplied_alpha as u32,
            pad: 0,
        };
        let path_globals = GlobalParams {
            premultiplied_alpha: 0,
            ..globals
        };
        let resources = &self.resources;
        let cache = &mut self.uniforms;
        let mut uploads = 0;
        let globals_changed = cache.globals.changes(globals);
        let path_globals_changed = cache.path_globals.changes(path_globals);
        if globals_changed || path_globals_changed {
            // Both viewport structures share one staging write.
            let mut bytes =
                vec![0_u8; self.path_globals_offset as usize + size_of::<GlobalParams>()];
            bytes[..size_of::<GlobalParams>()].copy_from_slice(bytemuck::bytes_of(&globals));
            bytes[self.path_globals_offset as usize..]
                .copy_from_slice(bytemuck::bytes_of(&path_globals));
            resources
                .queue
                .write_buffer(&resources.globals_buffer, 0, &bytes);
            uploads += 1;
        }
        if cache.gamma.changes(gamma) {
            resources.queue.write_buffer(
                &resources.globals_buffer,
                self.gamma_offset,
                bytemuck::bytes_of(&gamma),
            );
            uploads += 1;
        }
        self.stats.global_uploads += uploads;
    }

    /// Path rendering goes through an intermediate texture that must match the frame target's
    /// dimensions. It is created only when a frame contains paths (PRF-010 R2).
    fn ensure_intermediate_textures(&mut self, size: Size<DevicePixels>) {
        let width = size.width.0 as u32;
        let height = size.height.0 as u32;
        if self
            .resources
            .path_intermediate_texture
            .as_ref()
            .is_some_and(|texture| texture.width() == width && texture.height() == height)
        {
            return;
        }

        let format = self.target_format;
        let path_sample_count = self.rendering_params.path_sample_count;
        let resources = &mut self.resources;

        let (texture, view) =
            Self::create_path_intermediate(&resources.device, format, width, height);
        resources.path_intermediate_texture = Some(texture);
        resources.path_intermediate_view = Some(view);

        let (path_msaa_texture, path_msaa_view) = Self::create_msaa_if_needed(
            &resources.device,
            format,
            width,
            height,
            path_sample_count,
        )
        .map(|(texture, view)| (Some(texture), Some(view)))
        .unwrap_or((None, None));
        resources.path_msaa_texture = path_msaa_texture;
        resources.path_msaa_view = path_msaa_view;
        self.path_intermediate_generation += 1;
        self.stats.path_intermediate_allocations += 1;
    }

    /// Releases the path intermediates (PRF-010 R3, R5).
    pub(super) fn release_path_intermediates(&mut self) {
        let resources = &mut self.resources;
        if let Some(texture) = resources.path_intermediate_texture.take() {
            texture.destroy();
        }
        if let Some(texture) = resources.path_msaa_texture.take() {
            texture.destroy();
        }
        resources.path_intermediate_view = None;
        resources.path_msaa_view = None;
    }

    /// Explicit idle trim when a window parks (PRF-010 R5): path intermediates if the last
    /// frame was path-free, blur resources if the last frame had no effect. Never touches the
    /// atlas or the instance buffer.
    pub(super) fn trim(&mut self) {
        if self.path_free_frames >= 1 {
            self.release_path_intermediates();
        }
        self.resources.blur.trim();
    }

    fn rasterize_group(&self, fe: &mut FrameEncoder, group: &PathGroup, layout: &FrameLayout) {
        if group.vertices.is_empty() {
            return;
        }
        let resources = &self.resources;
        let Some(intermediate_view) = resources.path_intermediate_view.as_ref() else {
            return;
        };
        let vertices = layout.sections[SECTION_PATH_VERTICES];
        let key = InstanceKey {
            buffer_generation: self.instance_buffer_generation,
            offset: vertices.offset,
            size: vertices.size,
        };
        let Some(binding) = self.bindings.instances.get(&key) else {
            return;
        };
        let (target_view, resolve_target) = if let Some(msaa_view) = &resources.path_msaa_view {
            (msaa_view, Some(intermediate_view))
        } else {
            (intermediate_view, None)
        };
        fe.begin_pass(&wgpu::RenderPassDescriptor {
            label: Some("path_rasterization_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target_view,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            ..Default::default()
        });
        fe.set_pipeline(
            PipelineKind::PathRasterization,
            &resources.pipelines.path_rasterization,
        );
        fe.set_bind_group(0, PATH_GLOBALS_BINDING, &resources.path_globals_bind_group);
        fe.set_bind_group(1, binding.id, &binding.bind_group);
        // The path rasterization shader loads records by vertex index rather than instance
        // index.
        fe.pass().draw(group.vertices.clone(), 0..1);
        fe.end_pass();
    }

    /// Identity of the frame's globals bind group.
    fn globals_binding(&self) -> u64 {
        if self.resources.clip_bound {
            CLIP_GLOBALS_BINDING
        } else {
            GLOBALS_BINDING
        }
    }

    fn instance_entry(&self, section: Section) -> Option<&BindingEntry> {
        self.bindings.instances.get(&InstanceKey {
            buffer_generation: self.instance_buffer_generation,
            offset: section.offset,
            size: section.size,
        })
    }

    fn draw_instances(
        &self,
        fe: &mut FrameEncoder,
        kind: PipelineKind,
        pipeline: &wgpu::RenderPipeline,
        section: Section,
        range: Range<u32>,
    ) {
        if range.is_empty() {
            return;
        }
        let Some(binding) = self.instance_entry(section) else {
            return;
        };
        fe.set_pipeline(kind, pipeline);
        fe.set_bind_group(
            0,
            self.globals_binding(),
            &self.resources.globals_bind_group,
        );
        fe.set_bind_group(1, binding.id, &binding.bind_group);
        fe.pass().draw(0..4, range);
    }

    /// Draws one fill class of a quad range with its pipeline.
    fn draw_quad_run(
        &self,
        fe: &mut FrameEncoder,
        class: FillClass,
        section: Section,
        range: Range<usize>,
    ) {
        let pipelines = &self.resources.pipelines;
        let (kind, pipeline) = match class {
            FillClass::General => (PipelineKind::Quads, &pipelines.quads),
            FillClass::ClippedSolid => (
                PipelineKind::QuadsClippedSolid,
                &pipelines.quads_solid_clipped,
            ),
            FillClass::OpaqueSolid => (
                PipelineKind::QuadsOpaqueSolid,
                &pipelines.quads_solid_opaque,
            ),
        };
        self.draw_instances(fe, kind, pipeline, section, instance_range(range));
    }

    /// Draws a quad batch (PRF-008). Hardware adapters keep the batch whole and pick one route
    /// for it; software adapters draw contiguous runs of one class in paint order and split
    /// large bordered, rounded or faded solid quads into exterior strips and a solid interior.
    fn draw_quad_batch(
        &self,
        fe: &mut FrameEncoder,
        scene: &Scene,
        range: Range<usize>,
        section: Section,
        drawable: (u32, u32),
        fills: &mut Option<Vec<FillRecord>>,
    ) {
        let quads = &scene.quads[range.clone()];
        if !self.software_quad_fills {
            let class = batch_fill_class(quads);
            self.draw_quad_run(fe, class, section, range.clone());
            if let Some(fills) = fills {
                fills.push(FillRecord {
                    quads: range,
                    class,
                    draws: 1,
                    scissors: 0,
                    split: false,
                });
            }
            return;
        }

        let mut run: Option<(usize, FillClass)> = None;
        let flush = |fe: &mut FrameEncoder,
                     run: &mut Option<(usize, FillClass)>,
                     end: usize,
                     fills: &mut Option<Vec<FillRecord>>| {
            if let Some((start, class)) = run.take() {
                self.draw_quad_run(fe, class, section, start..end);
                if let Some(fills) = fills {
                    fills.push(FillRecord {
                        quads: start..end,
                        class,
                        draws: 1,
                        scissors: 0,
                        split: false,
                    });
                }
            }
        };
        for (offset, quad) in quads.iter().enumerate() {
            let index = range.start + offset;
            if let Some(interior) = large_interior(quad, drawable) {
                flush(fe, &mut run, index, fills);
                self.draw_split_quad(fe, quad, index, section, interior, drawable, fills);
                continue;
            }
            let class = quad_fill_class(quad);
            match run {
                Some((_, current)) if current == class => {}
                _ => {
                    flush(fe, &mut run, index, fills);
                    run = Some((index, class));
                }
            }
        }
        flush(fe, &mut run, range.end, fills);
    }

    /// Draws one quad as general shading through up to four disjoint exterior scissors and
    /// one solid draw limited to its integer interior, then restores the full scissor
    /// (PRF-008 R7).
    #[allow(clippy::too_many_arguments)]
    fn draw_split_quad(
        &self,
        fe: &mut FrameEncoder,
        quad: &Quad,
        index: usize,
        section: Section,
        interior: [u32; 4],
        drawable: (u32, u32),
        fills: &mut Option<Vec<FillRecord>>,
    ) {
        let [x0, y0, x1, y1] = interior;
        let (width, height) = drawable;
        // Left and right strips span the drawable's height; top and bottom strips the
        // interior's width.
        let strips = [
            [0, 0, x0, height],
            [x1, 0, width, height],
            [x0, 0, x1, y0],
            [x0, y1, x1, height],
        ];
        let mut draws = 0;
        let mut scissors = 0;
        for [left, top, right, bottom] in strips {
            if right <= left || bottom <= top {
                continue;
            }
            fe.pass()
                .set_scissor_rect(left, top, right - left, bottom - top);
            self.draw_quad_run(fe, FillClass::General, section, index..index + 1);
            draws += 1;
            scissors += 1;
        }
        let class = if quad.background.as_solid().map(|color| color.a) == Some(1.0)
            && quad.scope.curved_clip == 0
        {
            FillClass::OpaqueSolid
        } else {
            FillClass::ClippedSolid
        };
        fe.pass().set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        self.draw_quad_run(fe, class, section, index..index + 1);
        fe.pass().set_scissor_rect(0, 0, width, height);
        if let Some(fills) = fills {
            fills.push(FillRecord {
                quads: index..index + 1,
                class,
                draws: draws + 1,
                scissors: scissors + 2,
                split: true,
            });
        }
    }

    fn draw_textured(
        &self,
        fe: &mut FrameEncoder,
        kind: PipelineKind,
        pipeline: &wgpu::RenderPipeline,
        section: Section,
        texture: TextureKey,
        range: Range<u32>,
    ) {
        if range.is_empty() {
            return;
        }
        let Some(binding) = self.instance_entry(section) else {
            return;
        };
        // The atlas has released this texture; the batch belongs to a stale paint that will
        // be replaced once its view re-renders.
        let Some(texture) = self.bindings.textures.get(&texture) else {
            return;
        };
        fe.set_pipeline(kind, pipeline);
        fe.set_bind_group(
            0,
            self.globals_binding(),
            &self.resources.globals_bind_group,
        );
        fe.set_bind_group(1, binding.id, &binding.bind_group);
        fe.set_bind_group(2, texture.id, &texture.bind_group);
        fe.pass().draw(0..4, range);
    }

    fn atlas_key(&self, texture_id: AtlasTextureId) -> Option<TextureKey> {
        let generation = self.atlas_generations.get(&texture_id)?;
        Some(TextureKey::Atlas(texture_id, *generation))
    }

    /// Creates every binding the frame needs before recording starts, so recording reads the
    /// cache without mutating it.
    fn prepare_bindings(&mut self, scene: &Scene, scratch: &FrameScratch, layout: &FrameLayout) {
        self.bindings.begin_frame();
        let mut atlas_textures = SmallVec::<[AtlasTextureId; 8]>::new();
        for batch in &scratch.batches {
            let texture_id = match batch {
                PrimitiveBatch::MonochromeSprites { texture_id, .. }
                | PrimitiveBatch::SubpixelSprites { texture_id, .. }
                | PrimitiveBatch::PolychromeSprites { texture_id, .. } => *texture_id,
                _ => continue,
            };
            if vespertine::is_tile_texture(texture_id) {
                continue;
            }
            if !atlas_textures.contains(&texture_id) {
                atlas_textures.push(texture_id);
            }
        }
        self.atlas_generations.clear();
        for batch in &scratch.batches {
            if let PrimitiveBatch::PolychromeSprites { texture_id, range } = batch
                && vespertine::is_tile_texture(*texture_id)
                && let Some((serial, view)) = self.tile_texture_view(scene, range.start)
            {
                let view = view.clone();
                self.texture_binding(TextureKey::Tile(serial), &view);
            }
        }
        for texture_id in atlas_textures {
            let Some(info) = self.atlas.get_texture_info(texture_id) else {
                continue;
            };
            self.atlas_generations.insert(texture_id, info.generation);
            self.texture_binding(TextureKey::Atlas(texture_id, info.generation), &info.view);
        }

        let counts = [
            scene.quads.len(),
            scene.shadows.len(),
            scene.underlines.len(),
            scene.monochrome_sprites.len(),
            scene.subpixel_sprites.len(),
            scene.polychrome_sprites.len(),
            scratch.path_vertices.len(),
            scratch.path_sprites.len(),
        ];
        for (section, count) in layout.sections.iter().zip(counts) {
            if count > 0 {
                self.instance_binding(*section);
            }
        }
        if !scratch.path_batches.is_empty()
            && let Some(view) = self.resources.path_intermediate_view.clone()
        {
            let key = TextureKey::PathIntermediate(self.path_intermediate_generation);
            self.texture_binding(key, &view);
        }
    }

    pub(super) fn record_frame(
        &mut self,
        scene: &Scene,
        target_texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        clear_color: wgpu::Color,
        present_to: Option<&wgpu::TextureView>,
    ) -> Result<wgpu::SubmissionIndex> {
        let mut scratch = std::mem::take(&mut self.scratch);
        let result = self.record_frame_with(
            scene,
            &mut scratch,
            target_texture,
            target_view,
            clear_color,
            present_to,
        );
        self.scratch = scratch;
        result
    }

    fn record_frame_with(
        &mut self,
        scene: &Scene,
        scratch: &mut FrameScratch,
        target_texture: &wgpu::Texture,
        target_view: &wgpu::TextureView,
        clear_color: wgpu::Color,
        present_to: Option<&wgpu::TextureView>,
    ) -> Result<wgpu::SubmissionIndex> {
        self.prepare_clip_buffers(scene);
        Self::plan_batches(scene, scratch, self.path_group_limit);
        let has_paths = !scratch.path_batches.is_empty();
        if has_paths {
            let size = Size {
                width: DevicePixels(target_texture.width() as i32),
                height: DevicePixels(target_texture.height() as i32),
            };
            self.ensure_intermediate_textures(size);
        }
        let layout = self.frame_layout(scene, scratch);
        self.write_frame_data(scene, scratch, &layout)
            .with_context(|| {
                format!(
                    "scene too large: {} paths, {} shadows, {} quads, {} underlines, {} monochrome sprites, {} subpixel sprites, {} polychrome sprites",
                    scene.paths.len(),
                    scene.shadows.len(),
                    scene.quads.len(),
                    scene.underlines.len(),
                    scene.monochrome_sprites.len(),
                    scene.subpixel_sprites.len(),
                    scene.polychrome_sprites.len(),
                )
            })?;
        self.prepare_bindings(scene, scratch, &layout);
        self.resources.blur.begin_frame();

        let mut fe = FrameEncoder {
            encoder: self.resources.device.create_command_encoder(
                &wgpu::CommandEncoderDescriptor {
                    label: Some("main_encoder"),
                },
            ),
            pass: None,
            state: PassState::default(),
            render_passes: 0,
            pipeline_sets: 0,
            bind_group_sets: 0,
        };

        // The first group of path batches is rasterized before the main pass begins.
        let mut next_group = 0;
        if let Some(group) = scratch.path_groups.first() {
            self.rasterize_group(&mut fe, group, &layout);
            self.stats.path_rasterization_passes += u64::from(!group.vertices.is_empty());
            next_group = 1;
        }
        fe.begin_main_pass(target_view, wgpu::LoadOp::Clear(clear_color));

        let drawable = (target_texture.width(), target_texture.height());
        let mut fills = self.fill_log.take();
        if let Some(fills) = &mut fills {
            fills.clear();
        }
        let mut pending_blurs = scene.backdrop_blurs.iter().peekable();
        let mut path_batch_index = 0;
        let sections = layout.sections;
        for batch in &scratch.batches {
            // Effects whose order is at or below the first order of this batch snapshot
            // everything painted before it.
            let first_order = scene.batch_first_order(batch);
            while let Some(effect) = pending_blurs.next_if(|effect| effect.order <= first_order) {
                fe.end_pass();
                let plan =
                    self.resources
                        .blur
                        .render_effect(&mut fe.encoder, target_texture, effect);
                fe.render_passes += 2 * u64::from(plan.is_some());
                fe.begin_main_pass(target_view, wgpu::LoadOp::Load);
                if let Some(plan) = plan {
                    self.composite_effect(&mut fe, &plan);
                }
            }

            match batch {
                PrimitiveBatch::Quads(range) => self.draw_quad_batch(
                    &mut fe,
                    scene,
                    range.clone(),
                    sections[SECTION_QUADS],
                    drawable,
                    &mut fills,
                ),
                PrimitiveBatch::Shadows(range) => self.draw_instances(
                    &mut fe,
                    PipelineKind::Shadows,
                    &self.resources.pipelines.shadows,
                    sections[SECTION_SHADOWS],
                    instance_range(range.clone()),
                ),
                PrimitiveBatch::Underlines(range) => self.draw_instances(
                    &mut fe,
                    PipelineKind::Underlines,
                    &self.resources.pipelines.underlines,
                    sections[SECTION_UNDERLINES],
                    instance_range(range.clone()),
                ),
                PrimitiveBatch::Paths(_) => {
                    let plan = &scratch.path_batches[path_batch_index];
                    path_batch_index += 1;
                    if plan.group >= next_group {
                        // A later group is rasterized in the middle of the frame: the main pass
                        // ends, the group is rasterized into the cleared intermediate and the
                        // main pass resumes with its contents loaded.
                        fe.end_pass();
                        let group = &scratch.path_groups[plan.group];
                        self.rasterize_group(&mut fe, group, &layout);
                        self.stats.path_rasterization_passes +=
                            u64::from(!group.vertices.is_empty());
                        next_group = plan.group + 1;
                        fe.begin_main_pass(target_view, wgpu::LoadOp::Load);
                    }
                    if !plan.vertices.is_empty() {
                        self.draw_path_composite(&mut fe, plan, &layout);
                    }
                }
                PrimitiveBatch::MonochromeSprites { texture_id, range } => {
                    if let Some(key) = self.atlas_key(*texture_id) {
                        self.draw_textured(
                            &mut fe,
                            PipelineKind::MonoSprites,
                            &self.resources.pipelines.mono_sprites,
                            sections[SECTION_MONO_SPRITES],
                            key,
                            instance_range(range.clone()),
                        );
                    }
                }
                PrimitiveBatch::SubpixelSprites { texture_id, range } => {
                    if let Some(key) = self.atlas_key(*texture_id) {
                        let pipelines = &self.resources.pipelines;
                        let (kind, pipeline) = match pipelines.subpixel_sprites.as_ref() {
                            Some(pipeline) => (PipelineKind::SubpixelSprites, pipeline),
                            None => (PipelineKind::MonoSprites, &pipelines.mono_sprites),
                        };
                        let section = sections[SECTION_SUBPIXEL_SPRITES];
                        self.draw_textured(
                            &mut fe,
                            kind,
                            pipeline,
                            section,
                            key,
                            instance_range(range.clone()),
                        );
                    }
                }
                PrimitiveBatch::PolychromeSprites { texture_id, range } => {
                    let key = if vespertine::is_tile_texture(*texture_id) {
                        self.tile_texture_key(scene, range.start)
                    } else {
                        self.atlas_key(*texture_id)
                    };
                    if let Some(key) = key {
                        self.draw_textured(
                            &mut fe,
                            PipelineKind::PolySprites,
                            &self.resources.pipelines.poly_sprites,
                            sections[SECTION_POLY_SPRITES],
                            key,
                            instance_range(range.clone()),
                        );
                    }
                }
                PrimitiveBatch::Surfaces(range) => {
                    // Surface draws carry their own pipeline, bind groups and dynamic offsets,
                    // so they bypass the pass-state tracking and invalidate it.
                    let resources = &mut self.resources;
                    resources.surfaces.draw(
                        &resources.device,
                        &resources.queue,
                        &resources.pipelines.surfaces,
                        &resources.bind_group_layouts.surfaces,
                        &resources.globals_bind_group,
                        &scene.surfaces,
                        range.clone(),
                        fe.pass(),
                    );
                    fe.state.reset();
                }
            }
        }
        fe.end_pass();
        self.fill_log = fills;

        if let Some(present_to) = present_to {
            self.resources
                .blur
                .present_intermediate(&mut fe.encoder, present_to);
            fe.render_passes += 1;
        }

        self.resources.blur.flush_frame();
        let submission = self
            .resources
            .queue
            .submit(std::iter::once(fe.encoder.finish()));

        self.stats.frames += 1;
        self.stats.render_passes += fe.render_passes;
        self.stats.pipeline_sets += fe.pipeline_sets;
        self.stats.bind_group_sets += fe.bind_group_sets;

        self.bindings.end_frame();
        self.note_instance_usage(layout.total);
        if has_paths {
            self.path_free_frames = 0;
        } else {
            self.path_free_frames = (self.path_free_frames + 1).min(PATH_FREE_FRAME_LIMIT);
            if self.path_free_frames >= PATH_FREE_FRAME_LIMIT {
                self.release_path_intermediates();
            }
        }
        self.resources
            .blur
            .end_frame(!scene.backdrop_blurs.is_empty());
        self.report_memory();
        Ok(submission)
    }

    /// Emits the opt-in memory report when one is due (DEV-002).
    fn report_memory(&mut self) {
        let Self {
            memory,
            resources,
            atlas,
            instance_data_capacity,
            ..
        } = self;
        memory.frame_completed(|| {
            let (atlas_textures, atlas_bytes) = atlas.memory_usage();
            let resolve = resources
                .path_intermediate_texture
                .as_ref()
                .map(|texture| (texture.width(), texture.height()));
            let multisample = resources
                .path_msaa_texture
                .as_ref()
                .map(|texture| (texture.width(), texture.height(), texture.sample_count()));
            crate::memory_report::MemoryFigures {
                device_bytes: resources
                    .device
                    .generate_allocator_report()
                    .map(|report| report.total_allocated_bytes),
                atlas_textures,
                atlas_bytes,
                instance_capacities: vec![*instance_data_capacity],
                backdrop_bytes: resources.blur.retained_bytes(),
                path_bytes: crate::memory_report::path_figures(resolve, multisample),
            }
        });
    }

    fn composite_effect(&self, fe: &mut FrameEncoder, plan: &CompositePlan) {
        self.resources.blur.composite(fe.pass(), plan);
        // The composite binds its own layout in slot zero.
        fe.state.reset();
    }

    fn draw_path_composite(
        &self,
        fe: &mut FrameEncoder,
        plan: &PathBatchPlan,
        layout: &FrameLayout,
    ) {
        let resources = &self.resources;
        let Some(sprites) = self.instance_entry(layout.sections[SECTION_PATH_SPRITES]) else {
            return;
        };
        let Some(texture) = self.bindings.textures.get(&TextureKey::PathIntermediate(
            self.path_intermediate_generation,
        )) else {
            return;
        };
        fe.set_pipeline(PipelineKind::Paths, &resources.pipelines.paths);
        fe.set_bind_group(0, self.globals_binding(), &resources.globals_bind_group);
        fe.set_bind_group(1, sprites.id, &sprites.bind_group);
        fe.set_bind_group(2, texture.id, &texture.bind_group);
        fe.pass().draw(0..4, plan.sprites.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_encoder(core: &WgpuRendererCore) -> (FrameEncoder, wgpu::TextureView) {
        let device = &core.resources.device;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("state_test_target"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: core.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("state_test"),
        });
        (
            FrameEncoder {
                encoder,
                pass: None,
                state: PassState::default(),
                render_passes: 0,
                pipeline_sets: 0,
                bind_group_sets: 0,
            },
            view,
        )
    }

    #[test]
    fn redundant_state_commands_are_suppressed_and_pass_restarts_reset_tracking()
    -> anyhow::Result<()> {
        let renderer = WgpuHeadlessRenderer::new()?;
        let core = &renderer.core;
        let (mut fe, view) = frame_encoder(core);
        let resources = &core.resources;
        fe.begin_main_pass(&view, wgpu::LoadOp::Load);
        let draw_setup = |fe: &mut FrameEncoder| {
            fe.set_pipeline(PipelineKind::Quads, &resources.pipelines.quads);
            fe.set_bind_group(0, GLOBALS_BINDING, &resources.globals_bind_group);
            fe.set_bind_group(1, 77, &resources.path_globals_bind_group);
        };
        draw_setup(&mut fe);
        assert_eq!((fe.pipeline_sets, fe.bind_group_sets), (1, 2));
        draw_setup(&mut fe);
        assert_eq!(
            (fe.pipeline_sets, fe.bind_group_sets),
            (1, 2),
            "an identical draw issues no state commands"
        );
        // A pipeline with identical earlier layouts keeps slots 0 and 1 bound.
        fe.set_pipeline(PipelineKind::MonoSprites, &resources.pipelines.mono_sprites);
        fe.set_bind_group(0, GLOBALS_BINDING, &resources.globals_bind_group);
        fe.set_bind_group(1, 77, &resources.path_globals_bind_group);
        assert_eq!((fe.pipeline_sets, fe.bind_group_sets), (2, 2));
        // Switching to a pipeline without the texture slot drops that slot only.
        fe.set_bind_group(2, 5, &resources.globals_bind_group);
        assert_eq!(fe.bind_group_sets, 3);
        fe.set_pipeline(PipelineKind::Quads, &resources.pipelines.quads);
        fe.set_pipeline(PipelineKind::MonoSprites, &resources.pipelines.mono_sprites);
        fe.set_bind_group(2, 5, &resources.globals_bind_group);
        assert_eq!(fe.bind_group_sets, 4, "the texture slot must be set again");

        // Restarting the pass resets everything.
        fe.begin_main_pass(&view, wgpu::LoadOp::Load);
        let (pipelines, binds) = (fe.pipeline_sets, fe.bind_group_sets);
        fe.set_pipeline(PipelineKind::Quads, &resources.pipelines.quads);
        fe.set_bind_group(0, GLOBALS_BINDING, &resources.globals_bind_group);
        fe.set_bind_group(1, 77, &resources.path_globals_bind_group);
        assert_eq!(
            (fe.pipeline_sets - pipelines, fe.bind_group_sets - binds),
            (1, 2),
            "a restarted pass re-emits the pipeline and bindings"
        );
        fe.end_pass();
        Ok(())
    }
}
