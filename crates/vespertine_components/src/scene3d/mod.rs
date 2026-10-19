//! Retained interactive three-dimensional image surfaces (CMP-025).
//!
//! A [`SceneSurface`] shows a rendered scene as an ordinary image inside framework layout. The
//! scene itself (geometry, materials, annotations) comes from an external [`SceneProducer`]; the
//! surface owns the [`Camera`], the pointer interaction, asynchronous frame scheduling with
//! result identity ([`FrameCore`], shared with the shader surfaces of CMP-026), quality
//! selection and resource lifetime.

pub mod camera;
pub mod core;
pub mod quality;
pub mod surface;
pub mod types;

pub use camera::{Camera, OrbitController};
pub use core::{Delivery, Discard, Displayed, FrameCore};
pub use quality::{Quality, RendererKind, select_quality};
pub use surface::{
    Dispatch, FnProducer, SceneProducer, SceneSurface, SceneSurfaces, SurfaceHandle,
};
pub use types::{
    Annotation, InputContext, Label, LabelColor, LabelSize, Palette, PaletteId, RenderKey,
    RenderRequest, RenderResult, SceneFrame, TargetSlot, hovered_annotation,
};
