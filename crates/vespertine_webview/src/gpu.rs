//! Upload of shared-memory frames into a persistent GPU texture on the window renderer's shared
//! device (PLT-015 R13).

use crate::frames::{BufferFrame, Damage};
use std::sync::Arc;
use vespertine_wgpu::wgpu;

/// The persistent texture of one page and the device it lives on.
pub struct TextureSlot {
    device: Arc<wgpu::Device>,
    texture: Arc<wgpu::Texture>,
    width: u32,
    height: u32,
}

/// Uploads `buffer` into the page's persistent 8-bit BGRA texture and returns the texture.
///
/// A resize, scale change, generation change or a different device allocates a new texture and
/// uploads the full frame; otherwise only the damage rectangle is written, read from the mapped
/// buffer with the buffer's stride (bytes per row = stride, row length width x 4 within the
/// rectangle). `shadow` is the tightly packed CPU copy of the previous frame, updated in place
/// with the same rectangle (and recreated when the size changed); it backs the image path when a
/// rounded clip encloses the page.
pub fn upload(
    slot: &mut Option<TextureSlot>,
    device: &Arc<wgpu::Device>,
    queue: &wgpu::Queue,
    buffer: &BufferFrame,
    shadow: &mut Option<Vec<u8>>,
) -> Arc<wgpu::Texture> {
    let (width, height) = (buffer.width, buffer.height);
    let reuse = slot.as_ref().is_some_and(|existing| {
        Arc::ptr_eq(&existing.device, device)
            && existing.width == width
            && existing.height == height
    });
    let mut damage = buffer.damage;
    if !reuse {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("web_page_frame"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        *slot = Some(TextureSlot {
            device: device.clone(),
            texture: Arc::new(texture),
            width,
            height,
        });
        damage = Damage::full(width, height);
    }
    let texture = slot.as_ref().expect("texture slot exists").texture.clone();
    let length = width as usize * height as usize * 4;
    if shadow.as_ref().is_none_or(|bytes| bytes.len() != length) {
        *shadow = Some(vec![0; length]);
        damage = Damage::full(width, height);
    }
    let shadow = shadow.as_mut().expect("shadow exists");
    if damage.is_empty() {
        return texture;
    }
    let stride = buffer.stride() as usize;
    let pixels = buffer.pixels();
    let offset = damage.y as usize * stride + damage.x as usize * 4;
    let needed = stride * (damage.height as usize - 1) + damage.width as usize * 4;
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: damage.x,
                y: damage.y,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &pixels[offset..offset + needed],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride as u32),
            rows_per_image: Some(damage.height),
        },
        wgpu::Extent3d {
            width: damage.width,
            height: damage.height,
            depth_or_array_layers: 1,
        },
    );
    let row = damage.width as usize * 4;
    for y in damage.y as usize..(damage.y + damage.height) as usize {
        let source = y * stride + damage.x as usize * 4;
        let target = (y * width as usize + damage.x as usize) * 4;
        shadow[target..target + row].copy_from_slice(&pixels[source..source + row]);
    }
    texture
}
