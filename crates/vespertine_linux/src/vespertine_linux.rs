#![cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;

#[cfg(feature = "wayland")]
pub use linux::WaylandSolidProducer;
pub use linux::{SolidChild, SolidProducer, current_platform, linux_platform};
