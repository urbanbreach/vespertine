//! The scene trait and the scene registry.
//!
//! Scenes register themselves with [`register_scene!`] at the end of their source file; the
//! runner looks them up by identifier. Nothing else lists the scenes.

use vespertine::{AnyElement, App, Window};

pub use inventory;

/// One benchmark or fidelity scene. The per-frame update is a pure function of the frame index.
pub trait Scene: 'static {
    /// Logical window size in pixels (width, height).
    fn size(&self) -> (f32, f32);

    /// Frame index rendered by capture mode.
    fn capture_frame(&self) -> u64 {
        0
    }

    /// Applies the scene update for frame index `f` (idempotent when called again with the same
    /// `f`) and builds the root element for the frame.
    fn frame(&mut self, f: u64, window: &mut Window, cx: &mut App) -> AnyElement;
}

/// A registered scene: its identifier and its constructor.
pub struct SceneEntry {
    pub id: &'static str,
    pub create: fn() -> Box<dyn Scene>,
}

inventory::collect!(SceneEntry);

/// Registers a scene under `$id` (e.g. `"PERF-LIST"` or `"FID-RND-001-1"`); `$constructor` is a
/// `fn() -> impl Scene`. Use once per scene, at the end of the scene's source file.
macro_rules! register_scene {
    ($id:expr, $constructor:expr) => {
        $crate::scene::inventory::submit! {
            $crate::scene::SceneEntry {
                id: $id,
                create: || ::std::boxed::Box::new(($constructor)()),
            }
        }
    };
}
pub(crate) use register_scene;

/// Constructs the scene with the given identifier, or `None` when it is not registered.
pub fn create_scene(id: &str) -> Option<Box<dyn Scene>> {
    inventory::iter::<SceneEntry>
        .into_iter()
        .find(|entry| entry.id == id)
        .map(|entry| (entry.create)())
}
