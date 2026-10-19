//! Request, result and frame types of the retained 3-D surface (CMP-025 R2, R6, R7).

use super::quality::Quality;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vespertine::{App, Hsla, Pixels, Point, RenderImage, SharedString, Size, point};
use vespertine_theme::appearance::try_theme_of;

/// Extra hover distance beyond an annotation's radius.
pub const HOVER_TOLERANCE: f32 = 6.0;
/// Smallest annotation radius a producer supplies.
pub const MIN_ANNOTATION_RADIUS: f32 = 4.0;

/// A hoverable annotation: a position relative to the surface origin, a radius and its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    /// Centre in logical pixels relative to the surface origin.
    pub position: Point<Pixels>,
    /// Radius in logical pixels (at least 4).
    pub radius: f32,
    /// The text shown in the surface's top-left corner while hovered.
    pub text: SharedString,
}

/// Colour roles of a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelColor {
    /// Primary text.
    Text,
    /// Muted text.
    Muted,
    /// Accent.
    Accent,
    /// Success status.
    Success,
    /// Warning status.
    Warning,
    /// Danger status.
    Danger,
}

/// Size roles of a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LabelSize {
    /// 10 logical pixels.
    Small,
    /// 12 logical pixels.
    Medium,
    /// 14 logical pixels.
    Large,
}

impl LabelSize {
    /// Text size in logical pixels.
    pub fn pixels(self) -> f32 {
        match self {
            LabelSize::Small => 10.0,
            LabelSize::Medium => 12.0,
            LabelSize::Large => 14.0,
        }
    }
}

/// A text label painted over the image: centred horizontally on `position` and bottom-aligned
/// to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    /// Anchor in logical pixels relative to the surface origin.
    pub position: Point<Pixels>,
    /// The text.
    pub text: SharedString,
    /// Colour role.
    pub color: LabelColor,
    /// Size role.
    pub size: LabelSize,
}

/// The colours a producer draws with and the surface paints labels with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Primary text.
    pub text: Hsla,
    /// Muted text.
    pub muted: Hsla,
    /// Accent.
    pub accent: Hsla,
    /// Success.
    pub success: Hsla,
    /// Warning.
    pub warning: Hsla,
    /// Danger.
    pub danger: Hsla,
}

/// Identity of a palette: changes whenever any colour does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PaletteId(pub u64);

impl Palette {
    /// The neutral dark palette.
    pub fn neutral() -> Self {
        let rgb = |hex: u32| -> Hsla { vespertine::rgb(hex).into() };
        Self {
            text: rgb(0xF0F1F3),
            muted: rgb(0xA3A9B5),
            accent: rgb(0x4C8DF6),
            success: rgb(0x5FBF7F),
            warning: rgb(0xE0B050),
            danger: rgb(0xE5675F),
        }
    }

    /// The palette of the active theme, or the neutral one when no theme is installed.
    pub fn current(cx: &App) -> Self {
        match try_theme_of(cx) {
            Some(theme) => Self {
                text: theme.colors.text.to_hsla(),
                muted: theme.colors.text_muted.to_hsla(),
                accent: theme.accent.primary.to_hsla(),
                success: theme.colors.success.to_hsla(),
                warning: theme.colors.warning.to_hsla(),
                danger: theme.colors.danger.to_hsla(),
            },
            None => Self::neutral(),
        }
    }

    /// The colour of a role.
    pub fn color(&self, role: LabelColor) -> Hsla {
        match role {
            LabelColor::Text => self.text,
            LabelColor::Muted => self.muted,
            LabelColor::Accent => self.accent,
            LabelColor::Success => self.success,
            LabelColor::Warning => self.warning,
            LabelColor::Danger => self.danger,
        }
    }

    /// The identity of this palette.
    pub fn id(&self) -> PaletteId {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for color in [
            self.text,
            self.muted,
            self.accent,
            self.success,
            self.warning,
            self.danger,
        ] {
            for part in [color.h, color.s, color.l, color.a] {
                part.to_bits().hash(&mut hasher);
            }
        }
        PaletteId(hasher.finish())
    }
}

/// The description of a render: everything the producer's output depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderKey {
    /// Displayed yaw (the turntable included).
    pub yaw: f32,
    /// Pitch.
    pub pitch: f32,
    /// Zoom.
    pub zoom: f32,
    /// Logical size of the surface.
    pub size: Size<Pixels>,
    /// Pixel scale (device pixels per logical pixel).
    pub scale: f32,
    /// Sampling quality.
    pub quality: Quality,
    /// Identity of the theme palette.
    pub palette: PaletteId,
    /// Scene generation: bumped whenever the producer supplies a new drawing.
    pub generation: u64,
    /// Producer-defined input state hash ([`SceneProducer::input_key`](super::SceneProducer::input_key)):
    /// pointer, time or any other input the output depends on. Zero for producers without
    /// such inputs.
    pub input: u64,
}

impl RenderKey {
    /// The pixel size of the output image: `ceil(logical size x scale)` per axis.
    pub fn pixel_size(&self) -> (u32, u32) {
        pixel_size(self.size, self.scale)
    }
}

/// `ceil(logical x scale)` per axis; zero when the logical size is empty.
pub fn pixel_size(size: Size<Pixels>, scale: f32) -> (u32, u32) {
    let axis = |v: Pixels| {
        let v = f32::from(v);
        if v.is_finite() && v > 0.0 && scale.is_finite() && scale > 0.0 {
            (v * scale).ceil() as u32
        } else {
            0
        }
    };
    (axis(size.width), axis(size.height))
}

/// A render request: the key plus a per-surface monotonically increasing sequence number.
#[derive(Clone, Debug)]
pub struct RenderRequest {
    /// What to render.
    pub key: RenderKey,
    /// Sequence number of this request.
    pub sequence: u64,
    /// Identity of the surface the request belongs to.
    pub identity: u64,
    /// Seconds since the surface was created (the animation time).
    pub time: f32,
    /// The palette to draw with.
    pub palette: Palette,
    /// Whether reduced motion was requested when the request was made.
    pub reduce_motion: bool,
    /// The pointer relative to the surface origin while it is over the surface (or captured).
    pub pointer: Option<Point<Pixels>>,
    /// Where the primary button was pressed, relative to the surface origin, while it is held
    /// (only tracked when the surface tracks the pointer or orbits).
    pub press: Option<Point<Pixels>>,
    /// Retained producer targets (GPU textures and the like), reused between frames.
    pub targets: TargetSlot,
}

/// A finished frame.
#[derive(Clone, Debug)]
pub struct SceneFrame {
    /// Straight-alpha BGRA pixels, top row first, transparent where nothing is drawn.
    pub image: Arc<RenderImage>,
    /// Hoverable annotations in producer order.
    pub annotations: Vec<Annotation>,
    /// Labels painted over the image.
    pub labels: Vec<Label>,
}

/// The outcome of a request.
#[derive(Clone, Debug)]
pub struct RenderResult {
    /// Sequence number of the request.
    pub sequence: u64,
    /// The key the frame was rendered for.
    pub key: RenderKey,
    /// Identity of the surface the request belonged to.
    pub identity: u64,
    /// The frame, or the producer's error message.
    pub outcome: Result<SceneFrame, String>,
    /// How long the producer took.
    pub render_time: Duration,
}

impl RenderResult {
    /// A successful result for `request`.
    pub fn success(request: &RenderRequest, frame: SceneFrame, render_time: Duration) -> Self {
        Self {
            sequence: request.sequence,
            key: request.key,
            identity: request.identity,
            outcome: Ok(frame),
            render_time,
        }
    }

    /// A failed result for `request`.
    pub fn failure(
        request: &RenderRequest,
        message: impl Into<String>,
        render_time: Duration,
    ) -> Self {
        Self {
            sequence: request.sequence,
            key: request.key,
            identity: request.identity,
            outcome: Err(message.into()),
            render_time,
        }
    }
}

/// The hovered annotation: the one whose centre is nearest the pointer among those within
/// `radius + 6` px; ties resolve to the earliest in producer order.
pub fn hovered_annotation(
    annotations: &[Annotation],
    pointer: Option<Point<Pixels>>,
) -> Option<usize> {
    let pointer = pointer?;
    let mut best: Option<(usize, f32)> = None;
    for (index, annotation) in annotations.iter().enumerate() {
        let dx = f32::from(annotation.position.x) - f32::from(pointer.x);
        let dy = f32::from(annotation.position.y) - f32::from(pointer.y);
        let distance = dx.hypot(dy);
        if distance <= annotation.radius + HOVER_TOLERANCE && best.is_none_or(|(_, d)| distance < d)
        {
            best = Some((index, distance));
        }
    }
    best.map(|(index, _)| index)
}

/// What a producer sees when the surface asks whether its inputs changed.
#[derive(Clone, Copy, Debug)]
pub struct InputContext {
    /// The pointer relative to the surface origin, if it is over the surface.
    pub pointer: Option<Point<Pixels>>,
    /// The press origin, if the primary button is held.
    pub press: Option<Point<Pixels>>,
    /// Seconds since the surface was created.
    pub time: f32,
    /// Whether reduced motion is requested.
    pub reduce_motion: bool,
    /// Logical size of the surface.
    pub size: Size<Pixels>,
    /// Pixel scale.
    pub scale: f32,
}

/// A slot for resources a producer retains between frames (GPU textures, scratch buffers). The
/// surface clears it when it is released, removed or replaced; the producer recreates the
/// content when the pixel size changes.
#[derive(Clone, Default)]
pub struct TargetSlot(Arc<Mutex<Option<(Box<dyn std::any::Any + Send>, (u32, u32))>>>);

impl std::fmt::Debug for TargetSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetSlot")
            .field("allocated", &self.is_allocated())
            .finish()
    }
}

impl TargetSlot {
    /// Runs `f` on the retained value of type `T` for `size`; the value is created with `create`
    /// when the slot is empty, holds another type or was made for another pixel size.
    pub fn with<T: Send + 'static, R>(
        &self,
        size: (u32, u32),
        create: impl FnOnce() -> T,
        f: impl FnOnce(&mut T) -> R,
    ) -> R {
        let mut guard = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let reusable = guard
            .as_ref()
            .is_some_and(|(value, current)| *current == size && value.is::<T>());
        if !reusable {
            *guard = Some((Box::new(create()), size));
        }
        let (value, _) = guard.as_mut().expect("slot was just filled");
        f(value.downcast_mut::<T>().expect("slot holds a T"))
    }

    /// Whether the slot holds a value.
    pub fn is_allocated(&self) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).is_some()
    }

    /// Drops the retained value.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }
}

/// A new annotation position helper for producers.
pub fn annotation_at(x: f32, y: f32, radius: f32, text: impl Into<SharedString>) -> Annotation {
    Annotation {
        position: point(vespertine::px(x), vespertine::px(y)),
        radius: radius.max(MIN_ANNOTATION_RADIUS),
        text: text.into(),
    }
}
