//! FID-RND-006: one shared-device GPU texture composited by a styled surface element, with an
//! ordinary rectangle painted after it. The 4 x 4 device-pixel texture (left two columns red,
//! right two columns blue) is created on the window renderer's own device and never read back.

use crate::common::{WHITE, hex};
use crate::scene::{Scene, register_scene};
use std::sync::Arc;
use vespertine::{
    AnyElement, App, DevicePixels, IntoElement, ObjectFit, ParentElement, Styled, SurfaceSource,
    Window, div, px, size, surface,
};
use vespertine_wgpu::wgpu;

const TEXTURE_SIDE: u32 = 4;

/// Builds texture A: rows are identical, columns 0..2 opaque #FF0000 and 2..4 opaque #0000FF.
fn texture_a(window: &Window) -> Option<SurfaceSource> {
    let context = window.gpu_context()?;
    let (device, queue) = context.downcast::<wgpu::Device, wgpu::Queue>()?;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("fid_rnd_006_texture_a"),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        format: wgpu::TextureFormat::Rgba8Unorm,
        dimension: wgpu::TextureDimension::D2,
        view_formats: &[],
        mip_level_count: 1,
        sample_count: 1,
        size: wgpu::Extent3d {
            width: TEXTURE_SIDE,
            height: TEXTURE_SIDE,
            depth_or_array_layers: 1,
        },
    });
    let mut texels = Vec::with_capacity((TEXTURE_SIDE * TEXTURE_SIDE * 4) as usize);
    for _row in 0..TEXTURE_SIDE {
        for column in 0..TEXTURE_SIDE {
            texels.extend_from_slice(if column < 2 {
                &[0xFF, 0x00, 0x00, 0xFF]
            } else {
                &[0x00, 0x00, 0xFF, 0xFF]
            });
        }
    }
    queue.write_texture(
        texture.as_image_copy(),
        &texels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(TEXTURE_SIDE * 4),
            rows_per_image: Some(TEXTURE_SIDE),
        },
        wgpu::Extent3d {
            width: TEXTURE_SIDE,
            height: TEXTURE_SIDE,
            depth_or_array_layers: 1,
        },
    );
    Some(SurfaceSource::gpu_texture(
        Arc::new(texture),
        size(
            DevicePixels(TEXTURE_SIDE as i32),
            DevicePixels(TEXTURE_SIDE as i32),
        ),
    ))
}

struct SurfaceScene {
    source: Option<SurfaceSource>,
}

impl Scene for SurfaceScene {
    fn size(&self) -> (f32, f32) {
        (480.0, 240.0)
    }

    fn frame(&mut self, _f: u64, window: &mut Window, _cx: &mut App) -> AnyElement {
        if self.source.is_none() {
            self.source = texture_a(window);
        }
        let mut root = div().relative().size_full().bg(hex(WHITE));
        if let Some(source) = self.source.clone() {
            root = root.child(
                surface(source)
                    .object_fit(ObjectFit::Fill)
                    .absolute()
                    .left(px(40.))
                    .top(px(40.))
                    .w(px(160.))
                    .h(px(80.)),
            );
        }
        root.child(
            div()
                .absolute()
                .left(px(100.))
                .top(px(100.))
                .w(px(40.))
                .h(px(40.))
                .bg(hex(0x000000)),
        )
        .into_any_element()
    }
}

register_scene!("FID-RND-006", || SurfaceScene { source: None });
