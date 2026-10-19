use std::any::Any;
use std::sync::Arc;

/// A type-erased pair of shared graphics-library device and queue handles.
///
/// A window backend that renders with a GPU device exposes that device through
/// [`Window::gpu_context`](crate::Window::gpu_context) so applications can create textures on
/// the very device the window composites with (see
/// [`SurfaceSource::gpu_texture`](crate::SurfaceSource::gpu_texture)). The core crate does not
/// depend on the graphics library, so the handles are erased; the Linux backends store the
/// shared `wgpu::Device` and `wgpu::Queue` as `Arc<wgpu::Device>` and `Arc<wgpu::Queue>`.
///
/// Values are obtained from the window and never constructed by applications.
#[derive(Clone)]
pub struct SharedGpuContext {
    device: Arc<dyn Any + Send + Sync>,
    queue: Arc<dyn Any + Send + Sync>,
}

impl SharedGpuContext {
    /// Wraps a backend's shared device and queue. Intended for platform backends.
    pub fn new(device: Arc<dyn Any + Send + Sync>, queue: Arc<dyn Any + Send + Sync>) -> Self {
        Self { device, queue }
    }

    /// The erased shared device handle.
    pub fn device(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.device
    }

    /// The erased shared queue handle.
    pub fn queue(&self) -> &Arc<dyn Any + Send + Sync> {
        &self.queue
    }

    /// Recovers the concrete shared device and queue handles, retaining shared ownership of
    /// both. Returns `None` when either handle is not of the requested type.
    pub fn downcast<D, Q>(&self) -> Option<(Arc<D>, Arc<Q>)>
    where
        D: Any + Send + Sync,
        Q: Any + Send + Sync,
    {
        let device = Arc::clone(&self.device).downcast::<D>().ok()?;
        let queue = Arc::clone(&self.queue).downcast::<Q>().ok()?;
        Some((device, queue))
    }
}

impl std::fmt::Debug for SharedGpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedGpuContext").finish_non_exhaustive()
    }
}
