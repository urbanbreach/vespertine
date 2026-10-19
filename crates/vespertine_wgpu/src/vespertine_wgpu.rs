mod blur;
mod cosmic_text_system;
#[cfg(all(test, target_os = "linux"))]
mod image_update_tests;
pub mod memory_report;
mod wgpu_atlas;
mod wgpu_blur;
mod wgpu_context;
mod wgpu_renderer;
mod wgpu_surfaces;

pub use cosmic_text_system::*;
pub use wgpu;
pub use wgpu_atlas::*;
pub use wgpu_blur::{BlurResourceState, BlurStats};
pub use wgpu_context::*;
#[cfg(any(test, feature = "bench-support", feature = "test-support"))]
pub use wgpu_renderer::WgpuHeadlessRenderer;
pub use wgpu_renderer::{
    ClipStats, FillClass, FillRecord, GpuContext, RecordStats, WgpuRenderer, WgpuSurfaceConfig,
};

#[cfg(test)]
mod tests_fade;
#[cfg(test)]
mod tests_scope;

#[cfg(test)]
mod tests_clip;
#[cfg(test)]
mod tests_clip_window;
#[cfg(test)]
mod tests_glyph_blur;

#[cfg(test)]
mod tests_host;

#[cfg(test)]
mod tests_plate;

#[cfg(test)]
mod tests_canvas;

#[cfg(test)]
mod tests_image_mask;

#[cfg(test)]
mod tests_nearest;

#[cfg(test)]
mod tests_image_hosts;

#[cfg(test)]
mod tests_style_pixels;
