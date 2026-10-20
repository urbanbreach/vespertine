//! The device headless renderers share lives exactly as long as the renderers using it.
//!
//! A device that is still alive when the process exits is torn down by the GPU driver's exit
//! handler while the driver's own worker threads are running, which crashed test processes with
//! SIGSEGV after their last test had passed. The last renderer therefore destroys the device
//! when it is dropped. This binary holds a single test, so no other renderer of the process
//! interferes with the counts.

use std::sync::{Arc, Weak};

use vespertine_wgpu::{WgpuHeadlessRenderer, gpu_creation_counts};

/// The shared device of `renderer`, without keeping it alive.
fn device_of(renderer: &WgpuHeadlessRenderer) -> Weak<wgpu::Device> {
    let context = renderer
        .shared_gpu_context()
        .expect("a headless renderer on a healthy device shares its context");
    let (device, _queue) = context
        .downcast::<wgpu::Device, wgpu::Queue>()
        .expect("the shared context holds a wgpu device and queue");
    Arc::downgrade(&device)
}

#[test]
fn the_shared_headless_device_is_destroyed_with_its_last_renderer() {
    let before = gpu_creation_counts();
    let first = match WgpuHeadlessRenderer::new() {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("no headless GPU adapter, skipping: {error:#}");
            return;
        }
    };
    let second = WgpuHeadlessRenderer::new().expect("a second renderer");
    let device = device_of(&first);
    assert!(
        Weak::ptr_eq(&device, &device_of(&second)),
        "renderers alive together share one device"
    );
    assert_eq!(gpu_creation_counts().since(before).devices, 1);

    drop(first);
    assert!(
        device.upgrade().is_some(),
        "the device stays while a renderer uses it"
    );
    let third = WgpuHeadlessRenderer::new().expect("a renderer joining the live device");
    assert!(Weak::ptr_eq(&device, &device_of(&third)));
    assert_eq!(gpu_creation_counts().since(before).devices, 1);

    drop(second);
    drop(third);
    assert!(
        device.upgrade().is_none(),
        "the last renderer destroys the device instead of leaving it to the driver's exit handler"
    );
    let fourth = WgpuHeadlessRenderer::new().expect("a renderer after the device was released");
    assert_eq!(
        gpu_creation_counts().since(before).devices,
        2,
        "a new device was created for it"
    );
    let fresh = device_of(&fourth);
    assert!(!Weak::ptr_eq(&device, &fresh));
    drop(fourth);
    assert!(fresh.upgrade().is_none());
}
