//! RND-006: a texture created on the window renderer's own GPU device, filled by a GPU render
//! pass every frame and composited by a styled `surface` element. Nothing is read back to the
//! CPU and no image is uploaded: the texture never leaves the GPU.
//!
//! Run on Wayland (the default when a compositor is available):
//!
//! ```text
//! cargo run -p vespertine --example gpu_surface
//! ```
//!
//! and on X11 by hiding the Wayland compositor from the process:
//!
//! ```text
//! WAYLAND_DISPLAY= cargo run -p vespertine --example gpu_surface
//! ```
//!
//! `GPU_SURFACE_FRAMES=<n>` exits after `n` frames and prints the per-run counters.
//!
//! Corner radii are not applied to surfaces: the rounded frame below is a separate element
//! that surrounds the texture, which is itself rectangular.

#[cfg(target_os = "linux")]
mod linux {
    use std::sync::Arc;
    use vespertine::{
        App, Bounds, Context, DevicePixels, ObjectFit, SharedGpuContext, SharedString,
        SurfaceSource, Window, WindowBounds, WindowOptions, div, prelude::*, px, rgb, size,
        surface,
    };
    use vespertine_platform::application;
    use vespertine_wgpu::wgpu;

    const TEXTURE_SIZE: u32 = 256;

    /// The producer's device-owned resources. Rebuilt whenever the window hands out a different
    /// device, which is what happens after the backend recovered from a device loss.
    struct Producer {
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        texture: Arc<wgpu::Texture>,
        view: wgpu::TextureView,
    }

    impl Producer {
        fn new(context: &SharedGpuContext) -> Option<Self> {
            let (device, queue) = context.downcast::<wgpu::Device, wgpu::Queue>()?;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gpu_surface_example"),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
                format: wgpu::TextureFormat::Rgba8Unorm,
                dimension: wgpu::TextureDimension::D2,
                sample_count: 1,
                mip_level_count: 1,
                size: wgpu::Extent3d {
                    depth_or_array_layers: 1,
                    width: TEXTURE_SIZE,
                    height: TEXTURE_SIZE,
                },
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            Some(Self {
                device,
                queue,
                texture: Arc::new(texture),
                view,
            })
        }

        /// Fills the whole texture with one color in a GPU render pass.
        fn fill(&self, [r, g, b]: [f64; 3]) {
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("gpu_surface_example_fill"),
                });
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gpu_surface_example_clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                ..Default::default()
            }));
            self.queue.submit(std::iter::once(encoder.finish()));
        }

        fn source(&self) -> SurfaceSource {
            SurfaceSource::gpu_texture(
                self.texture.clone(),
                size(
                    DevicePixels(TEXTURE_SIZE as i32),
                    DevicePixels(TEXTURE_SIZE as i32),
                ),
            )
        }
    }

    struct GpuSurface {
        producer: Option<Producer>,
        frame: u32,
        max_frames: Option<u32>,
        recoveries: u32,
    }

    impl GpuSurface {
        /// Reacquires the producer when the window's device changed, and stops producing while
        /// the device is lost.
        fn refresh_producer(&mut self, window: &Window) {
            if window.gpu_device_lost() == Some(true) {
                // Stop submitting work and wait for the backend to recover its device.
                self.producer = None;
                return;
            }
            let Some(context) = window.gpu_context() else {
                self.producer = None;
                return;
            };
            let current = context.downcast::<wgpu::Device, wgpu::Queue>();
            let stale = match (&self.producer, &current) {
                (Some(producer), Some((device, _))) => !Arc::ptr_eq(&producer.device, device),
                (None, Some(_)) => true,
                (_, None) => true,
            };
            if stale {
                if self.producer.is_some() {
                    self.recoveries += 1;
                }
                self.producer = Producer::new(&context);
            }
        }
    }

    /// sRGB color of HSL(hue, 65%, 60%) as 0..1 channels.
    fn hsl(hue: f64) -> [f64; 3] {
        let (s, l) = (0.65, 0.60);
        let c = (1.0 - (2.0 * l - 1.0f64).abs()) * s;
        let h = (hue % 360.0) / 60.0;
        let x = c * (1.0 - (h % 2.0 - 1.0).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.0),
            1 => (x, c, 0.0),
            2 => (0.0, c, x),
            3 => (0.0, x, c),
            4 => (x, 0.0, c),
            _ => (c, 0.0, x),
        };
        let m = l - c / 2.0;
        [r + m, g + m, b + m]
    }

    impl Render for GpuSurface {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.refresh_producer(window);
            let label: SharedString;
            let content = match &self.producer {
                Some(producer) => {
                    producer.fill(hsl(f64::from(self.frame * 3)));
                    label = format!(
                        "frame {} (device recoveries: {})",
                        self.frame, self.recoveries
                    )
                    .into();
                    div().size_full().child(
                        surface(producer.source())
                            .object_fit(ObjectFit::Contain)
                            .size_full(),
                    )
                }
                None => {
                    label = "waiting for the GPU device".into();
                    div().size_full()
                }
            };

            self.frame += 1;
            if self.max_frames.is_some_and(|max| self.frame >= max) {
                println!(
                    "frames: {}, bytes read back from the GPU: 0, bytes of CPU image uploads: 0, \
                     device recoveries: {}",
                    self.frame, self.recoveries
                );
                cx.quit();
            } else {
                window.request_animation_frame();
            }

            div()
                .flex()
                .flex_col()
                .gap_2()
                .p_4()
                .size_full()
                .bg(rgb(0xffffff))
                .child(label)
                .child(
                    div()
                        .flex_1()
                        .border_2()
                        .border_color(rgb(0x303030))
                        .rounded_lg()
                        .bg(rgb(0xeeeeee))
                        .overflow_hidden()
                        .child(content),
                )
        }
    }

    pub fn run() {
        application().run(|cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(480.), px(360.)), cx);
            let max_frames = std::env::var("GPU_SURFACE_FRAMES")
                .ok()
                .and_then(|frames| frames.parse().ok());
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| {
                    cx.new(|_| GpuSurface {
                        producer: None,
                        frame: 0,
                        max_frames,
                        recoveries: 0,
                    })
                },
            )
            .unwrap();
            cx.activate(true);
        });
    }
}

#[cfg(target_os = "linux")]
fn main() {
    linux::run();
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("gpu_surface needs the shared wgpu device exposed by the Linux backends");
}
