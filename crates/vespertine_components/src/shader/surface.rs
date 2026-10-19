//! The shader image surface (CMP-026): the adapter that configures a GPU program, its feedback
//! buffers, inputs, textures, blend mode and bloom, and presents the result as an ordinary
//! image.
//!
//! A [`ShaderSurface`] is a [`SceneSurface`] whose producer runs the GPU renderer: the camera
//! (CMP-025 R3, R4: defaults 0, 0, 1.0, orbit off), the one-in-flight scheduling, the result
//! identity (sequence numbers, discard of removed, replaced and out-of-order results), error
//! state and resource lifetime are those of the retained 3-D surface. Changing the program text
//! under the same identity replaces the surface in place: feedback state is reset and the
//! previous frame stays visible until the first frame of the new program lands.

use super::blend::{BlendMode, clamp_bloom};
use super::gpu::{GpuState, ShaderFrame, TextureSource, TextureTable};
use super::inputs::{mouse_table, render_size};
use super::program::{self, ProgramError};
use crate::scene3d::{
    Dispatch, InputContext, RenderRequest, RendererKind, SceneFrame, SceneProducer, SceneSurface,
};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use vespertine::{ElementId, IntoElement, Pixels, RenderImage};

/// Holds the retained GPU state in the surface's target slot.
struct GpuCell(Option<GpuState>);

struct ShaderProducer {
    program: Arc<str>,
    buffers: Vec<Arc<str>>,
    input_names: Vec<String>,
    input_values: Vec<f32>,
    texture_names: Vec<String>,
    textures: Arc<dyn TextureSource>,
    blend: BlendMode,
    bloom: f32,
    animate: bool,
    on_error: Option<Arc<dyn Fn(&str) + Send + Sync>>,
    static_hash: u64,
}

impl ShaderProducer {
    fn mouse(
        &self,
        pointer: Option<vespertine::Point<Pixels>>,
        press: Option<vespertine::Point<Pixels>>,
        size: vespertine::Size<Pixels>,
        scale: f32,
    ) -> [f32; 4] {
        let device = render_size(size, scale);
        mouse_table(pointer, press, device.1 as f32, scale)
    }
}

impl SceneProducer for ShaderProducer {
    fn render(&self, request: &RenderRequest) -> Result<SceneFrame, String> {
        let size = render_size(request.key.size, request.key.scale);
        let animated = self.animate && !request.reduce_motion;
        let frame = ShaderFrame {
            main: self.program.clone(),
            buffers: self.buffers.clone(),
            input_names: self.input_names.clone(),
            input_values: self.input_values.clone(),
            texture_names: self.texture_names.clone(),
            textures: self.textures.clone(),
            blend: self.blend,
            bloom: self.bloom,
            animated,
            size,
            ratio: request.key.scale,
            mouse: self.mouse(
                request.pointer,
                request.press,
                request.key.size,
                request.key.scale,
            ),
            camera: [request.key.yaw, request.key.pitch, request.key.zoom],
            time: request.time,
        };
        let bgra = request.targets.with(
            (0, 0),
            || GpuCell(None),
            |cell| {
                if cell.0.is_none() {
                    cell.0 = Some(GpuState::new()?);
                }
                cell.0.as_mut().expect("just created").render(&frame)
            },
        )?;
        let image =
            RenderImage::from_bgra(size.0, size.1, bgra).ok_or("the frame has no pixels")?;
        Ok(SceneFrame {
            image: Arc::new(image),
            annotations: Vec::new(),
            labels: Vec::new(),
        })
    }

    fn report_error(&self, message: &str) {
        if let Some(on_error) = &self.on_error {
            on_error(message);
        }
    }

    fn new_drawing(&self, _time: f32) -> bool {
        false
    }

    fn input_key(&self, context: &InputContext) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.static_hash.hash(&mut hasher);
        for value in &self.input_values {
            value.to_bits().hash(&mut hasher);
        }
        for value in self.mouse(context.pointer, context.press, context.size, context.scale) {
            value.to_bits().hash(&mut hasher);
        }
        // Time moves the output of an animated surface and any surface with feedback buffers,
        // while motion is permitted.
        if !context.reduce_motion && (self.animate || !self.buffers.is_empty()) {
            context.time.to_bits().hash(&mut hasher);
        }
        hasher.finish()
    }

    fn animating(&self, reduce_motion: bool) -> bool {
        !reduce_motion && (self.animate || !self.buffers.is_empty())
    }
}

/// A GPU-program adapter: the main program text, optional ordered buffer programs, named float
/// inputs, texture names, blend mode, bloom strength, animate flag, orbit, spin and camera
/// options and the surface height.
pub struct ShaderSurface {
    key: ElementId,
    program: Arc<str>,
    buffers: Vec<Arc<str>>,
    inputs: Vec<(String, f32)>,
    textures: Vec<String>,
    assets: Arc<dyn TextureSource>,
    blend: BlendMode,
    bloom: f32,
    animate: bool,
    orbit: bool,
    spin: f32,
    camera: (f32, f32, f32),
    zoom_step: f32,
    height: Pixels,
    identity: u64,
    dispatch: Dispatch,
    on_error: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl ShaderSurface {
    /// A surface keyed by `key` (stable across frames, unique within its window) running
    /// `program` as its image pass.
    pub fn new(key: impl Into<ElementId>, program: impl Into<Arc<str>>) -> Self {
        Self {
            key: key.into(),
            program: program.into(),
            buffers: Vec::new(),
            inputs: Vec::new(),
            textures: Vec::new(),
            assets: TextureTable::new(),
            blend: BlendMode::Light,
            bloom: 0.0,
            animate: false,
            orbit: false,
            spin: 0.0,
            camera: (0.0, 0.0, 1.0),
            zoom_step: crate::scene3d::camera::DEFAULT_ZOOM_STEP,
            height: vespertine::px(crate::scene3d::surface::DEFAULT_HEIGHT),
            identity: 0,
            dispatch: Dispatch::Background,
            on_error: None,
        }
    }

    /// The ordered buffer pass programs (at most 4). Buffer `j` is channel `j`.
    pub fn buffers(mut self, buffers: impl IntoIterator<Item = impl Into<Arc<str>>>) -> Self {
        self.buffers = buffers.into_iter().map(Into::into).collect();
        self
    }

    /// Named float inputs, exposed under the given identifiers (at most 16).
    pub fn inputs(mut self, inputs: impl IntoIterator<Item = (impl Into<String>, f32)>) -> Self {
        self.inputs = inputs.into_iter().map(|(n, v)| (n.into(), v)).collect();
        self
    }

    /// Names of the image textures (at most 4), resolved against the asset table.
    pub fn textures(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.textures = names.into_iter().map(Into::into).collect();
        self
    }

    /// The host's asset table texture names resolve against.
    pub fn asset_table(mut self, table: Arc<dyn TextureSource>) -> Self {
        self.assets = table;
        self
    }

    /// The blend mode (default Light).
    pub fn blend(mut self, blend: BlendMode) -> Self {
        self.blend = blend;
        self
    }

    /// Bloom strength, clamped to [0, 3] (default 0: no bloom).
    pub fn bloom(mut self, strength: f32) -> Self {
        self.bloom = clamp_bloom(strength);
        self
    }

    /// Whether the program is time-varying: an animated surface requests animation frames while
    /// visible and motion is permitted.
    pub fn animate(mut self, animate: bool) -> Self {
        self.animate = animate;
        self
    }

    /// Enables orbit dragging (default disabled for shader surfaces).
    pub fn orbit(mut self, orbit: bool) -> Self {
        self.orbit = orbit;
        self
    }

    /// Turntable in radians per second, clamped to [-3, 3].
    pub fn spin(mut self, spin: f32) -> Self {
        self.spin = spin;
        self
    }

    /// The initial camera (defaults 0, 0, 1.0).
    pub fn camera(mut self, yaw: f32, pitch: f32, zoom: f32) -> Self {
        self.camera = (yaw, pitch, zoom);
        self
    }

    /// Zoom factor per wheel notch.
    pub fn zoom_step(mut self, step: f32) -> Self {
        self.zoom_step = step;
        self
    }

    /// Logical height, clamped to 40 to 720.
    pub fn height(mut self, height: impl Into<Pixels>) -> Self {
        self.height = height.into();
        self
    }

    /// The identity of the surface; a different identity under the same key replaces it and
    /// releases everything the old one held.
    pub fn identity(mut self, identity: u64) -> Self {
        self.identity = identity;
        self
    }

    /// Where frames render (default: the background executor).
    pub fn dispatch(mut self, dispatch: Dispatch) -> Self {
        self.dispatch = dispatch;
        self
    }

    /// The error callback: receives the message of every failed frame (validation, compilation,
    /// device or readback failure, or "no GPU is available").
    pub fn on_error(mut self, callback: impl Fn(&str) + Send + Sync + 'static) -> Self {
        self.on_error = Some(Arc::new(callback));
        self
    }

    fn input_names(&self) -> Vec<String> {
        self.inputs.iter().map(|(name, _)| name.clone()).collect()
    }

    /// Validates the main program and every buffer program as the GPU will compile them: the
    /// first error, with its line in the author's own text.
    pub fn validate(&self) -> Result<(), ProgramError> {
        let names = self.input_names();
        if self.buffers.len() > program::MAX_BUFFERS {
            return Err(ProgramError {
                line: None,
                message: format!(
                    "{} buffers exceed the limit of {}",
                    self.buffers.len(),
                    program::MAX_BUFFERS
                ),
            });
        }
        if self.textures.len() > program::MAX_TEXTURES {
            return Err(ProgramError {
                line: None,
                message: format!(
                    "{} textures exceed the limit of {}",
                    self.textures.len(),
                    program::MAX_TEXTURES
                ),
            });
        }
        for (index, text) in self.buffers.iter().enumerate() {
            program::validate_program(text, &names).map_err(|mut error| {
                error.message = format!("buffer {index}: {}", error.message);
                error
            })?;
        }
        program::validate_program(&self.program, &names)
    }
}

fn program_epoch(program: &str, buffers: &[Arc<str>]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    program.hash(&mut hasher);
    for buffer in buffers {
        buffer.hash(&mut hasher);
    }
    hasher.finish()
}

impl IntoElement for ShaderSurface {
    type Element = SceneSurface;

    fn into_element(self) -> SceneSurface {
        let epoch = program_epoch(&self.program, &self.buffers);
        let mut static_hasher = std::collections::hash_map::DefaultHasher::new();
        self.blend.hash(&mut static_hasher);
        self.bloom.to_bits().hash(&mut static_hasher);
        self.input_names().hash(&mut static_hasher);
        self.textures.hash(&mut static_hasher);
        epoch.hash(&mut static_hasher);
        let producer = ShaderProducer {
            program: self.program.clone(),
            buffers: self.buffers.clone(),
            input_names: self.input_names(),
            input_values: self.inputs.iter().map(|(_, v)| *v).collect(),
            texture_names: self.textures.clone(),
            textures: self.assets.clone(),
            blend: self.blend,
            bloom: self.bloom,
            animate: self.animate,
            on_error: self.on_error.clone(),
            static_hash: static_hasher.finish(),
        };
        SceneSurface::new(self.key, producer)
            .height(self.height)
            .orbit(self.orbit)
            .spin(self.spin)
            .camera(self.camera.0, self.camera.1, self.camera.2)
            .zoom_step(self.zoom_step)
            .identity(self.identity)
            .epoch(epoch)
            .track_pointer(true)
            .dispatch(self.dispatch)
            .renderer(RendererKind::Gpu)
            .max_in_flight(1)
    }
}
